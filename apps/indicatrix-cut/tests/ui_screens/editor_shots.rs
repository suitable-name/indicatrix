//! The shots of the editor: Solid and Diagram views, the Retarget for Material dialog, the gear
//! change (the Confirm Index Remap dialog and the before/after composition) and the Optimize tab.

use crate::{
    kit::{Sc, card_rect, click_last, find_inherits, press_on, rect_of},
    nav::{CROWN_TIER, FVS, STUDIO},
    support::app::pump,
};
use indicatrix_cut::{CompareModel, EditorModel, OptimizeModel, RetargetModel, SolidPreviewModel};
use indicatrix_editor::material::gear_index_from_teeth;
use slint::{ComponentHandle, Model};
use slint_harness::screenshot::Shot;

/// The gear the `SplitChecker` is changed to.
const NEW_TEETH: i32 = 64;
/// How far the Optimize shot scrolls the inspector down to the candidate list, in logical pixels.
const OPTIMIZER_SCROLL: f32 = 900.0;
/// The material the Retarget shots move the design to.
const RETARGET_TARGET: &str = "emerald";
/// Space around and between the parts of the composed shot, in physical pixels.
const GAP: u32 = 48;
/// Status words of the compare pane while it still works on a frame.
const BUSY_WORDS: [&str; 5] = ["tracing", "solving", "rendering", "waiting", "building"];

/// `cad.png`: the Edit view in Solid shading with a crown tier selected.
///
/// # Errors
///
/// The design does not open, or has no crown tier.
pub fn cad(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(0);
    sc.set_inspector_tab(0);
    sc.select_tier(CROWN_TIER)?;
    sc.snap_main()
}

/// `cad_solved_diagram.png`: the Diagram view with the same tier selected.
///
/// # Errors
///
/// As [`cad`].
pub fn cad_solved_diagram(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_inspector_tab(0);
    sc.select_tier(CROWN_TIER)?;
    sc.set_view_mode(3);
    pump(1000);
    sc.snap_main()
}

/// `diagram_view.png`: the Diagram view with all panels and the Schedule tab.
///
/// # Errors
///
/// The design does not open.
pub fn diagram_view(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(3);
    sc.set_inspector_tab(3);
    pump(1000);
    sc.snap_main()
}

/// `diagram_view_detail.png`: the Diagram view with the Crown panel enlarged.
///
/// # Errors
///
/// The design does not open.
pub fn diagram_view_detail(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(3);
    sc.set_inspector_tab(3);
    let ui = sc.ui();
    let solid = ui.global::<SolidPreviewModel>();
    let _ = Sc::wait(30, || solid.get_has_diagram());
    pump(1000);
    let three_panels = solid.get_diagram_image();
    solid.set_diagram_enlarged_panel(0);
    // The enlarged panel is a replan of the diagram (what the pill's `changed` handler
    // asks for); asked for explicitly too, then wait for the new picture.
    solid.invoke_diagram_enlarged_panel_changed();
    if !Sc::wait(30, || solid.get_diagram_image() != three_panels) {
        return Err("the enlarged Crown panel never arrived".to_owned());
    }
    pump(1000);
    sc.snap_main()
}

/// Whether a compare status line says a frame is still being made.
pub fn busy_status(status: &str) -> bool {
    let lower = status.to_lowercase();
    BUSY_WORDS.iter().any(|word| lower.contains(word))
}

/// Opens the Retarget for Material dialog on the Emerald proposal in Shift mode.
fn open_retarget(sc: &mut Sc<'_, '_>) -> Result<(), String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    let retarget = ui.global::<RetargetModel>();
    retarget.invoke_open();
    if !Sc::wait(30, || retarget.get_is_open()) {
        return Err("the Retarget dialog did not open".to_owned());
    }
    let options: Vec<String> = retarget
        .get_material_options()
        .iter()
        .map(String::from)
        .collect();
    let index = options
        .iter()
        .position(|name| name.to_lowercase().contains(RETARGET_TARGET))
        .ok_or_else(|| format!("the dialog offers no {RETARGET_TARGET} among {options:?}"))?;
    retarget.set_mode_index(0);
    retarget.set_target_material_index(i32::try_from(index).unwrap_or(0));
    retarget.invoke_proposal_changed();
    pump(800);
    let compare = ui.global::<CompareModel>();
    if !Sc::wait(60, || compare.get_embedded_open()) {
        return Err("the comparison pane of the Retarget dialog did not open".to_owned());
    }
    pump(500);
    Ok(())
}

/// Waits until the embedded pane shows a finished frame; a timeout is a note.
fn settle_pane(sc: &mut Sc<'_, '_>) {
    let ui = sc.ui();
    let compare = ui.global::<CompareModel>();
    let retarget = ui.global::<RetargetModel>();
    let seconds = sc.cfg.render_timeout.as_secs();
    let settled = Sc::wait(seconds, || {
        compare.get_embedded_open()
            && compare.get_after_image().size().width > 0
            && !retarget.get_is_busy()
            && retarget.get_validity_state() != 1
            && !busy_status(compare.get_status().as_str())
    });
    if !settled {
        sc.note(format!(
            "the Retarget comparison did not finish within {seconds} s; photographed anyway"
        ));
    }
    pump(800);
}

/// One Retarget dialog shot with the layout pill and the renderer pill pressed.
fn retarget_shot(sc: &mut Sc<'_, '_>, layout: &str, renderer: &str) -> Result<Shot, String> {
    open_retarget(sc)?;
    for label in [layout, renderer] {
        if !click_last(&sc.view(), label) {
            return Err(format!("the Retarget dialog has no {label:?} button"));
        }
        pump(400);
    }
    settle_pane(sc);
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "RetargetDialog")?.crop(&shot))
}

/// `retarget_dialog.png`: Side by Side, Path Traced.
///
/// # Errors
///
/// The design does not open or the dialog is not as expected.
pub fn retarget_dialog(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    retarget_shot(sc, "Side by Side", "Path Traced")
}

/// `retarget_dialog_1.png`: Overlay, Solid.
///
/// # Errors
///
/// As [`retarget_dialog`].
pub fn retarget_dialog_1(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    retarget_shot(sc, "Overlay", "Solid")
}

/// Asks for the gear change to 64 teeth and waits for the Confirm Index Remap dialog.
fn request_gear_change(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    editor.invoke_gear_apply(
        gear_index_from_teeth(NEW_TEETH),
        NEW_TEETH.to_string().into(),
    );
    if !Sc::wait(30, || editor.get_gear_remap_open()) {
        return Err("the gear change did not open the Confirm Index Remap dialog".to_owned());
    }
    pump(600);
    Ok(())
}

/// `retarget_gear_2.png`: the Confirm Index Remap dialog for 96 -> 64 teeth.
///
/// # Errors
///
/// The design does not open or the dialog does not appear.
pub fn retarget_gear_2(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(STUDIO)?;
    request_gear_change(sc)?;
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "GearRemapDialog")?.crop(&shot))
}

/// The viewport and the tier table of the Edit view as two pictures.
fn merge_part(sc: &mut Sc<'_, '_>) -> Result<(Shot, Shot), String> {
    let shot = sc.snap_main()?;
    let view = sc.view();
    let top = rect_of(&view, "SolidViewportView").or_else(|_| rect_of(&view, "GemViewportView"))?;
    let table = rect_of(&view, "EditorTierTable")?;
    Ok((top.crop(&shot), table.crop(&shot)))
}

/// Puts the before and after parts side by side, the viewport over the tier table.
fn compose(before: &(Shot, Shot), after: &(Shot, Shot)) -> Shot {
    let parts = [&before.0, &before.1, &after.0, &after.1];
    let column = parts.iter().map(|part| part.width()).max().unwrap_or(1);
    let top = before.0.height().max(after.0.height());
    let table = before.1.height().max(after.1.height());
    let mut out = Shot::blank(
        GAP * 3 + column * 2,
        GAP * 3 + top + table,
        before.0.scale(),
    );
    // The site's figure background (#111111); `Shot::blank` alone would be transparent.
    out.fill(slint::Rgba8Pixel::new(0x11, 0x11, 0x11, 0xff));
    for (index, pair) in [before, after].into_iter().enumerate() {
        let left = GAP + u32::try_from(index).unwrap_or(0) * (column + GAP);
        out.paste(&pair.0, left + (column - pair.0.width()) / 2, GAP);
        out.paste(&pair.1, left + (column - pair.1.width()) / 2, GAP * 2 + top);
    }
    out
}

/// `retarget_merge.png`: the gear change 96 -> 64 before (left) and after (right).
///
/// # Errors
///
/// The design does not open, the change is not applied or the parts are not on screen.
pub fn retarget_merge(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(STUDIO)?;
    sc.set_view_mode(0);
    let before = merge_part(sc)?;
    request_gear_change(sc)?;
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    editor.invoke_gear_remap_confirm();
    if !Sc::wait(30, || !editor.get_gear_remap_open()) {
        return Err("the Confirm Index Remap dialog did not close".to_owned());
    }
    pump(500);
    let solved = Sc::wait(180, || {
        !editor.get_solve_running() && editor.get_solve_state().as_str() == "solved"
    });
    if !solved {
        return Err("the design did not solve after the gear change".to_owned());
    }
    sc.set_view_mode(0);
    let after = merge_part(sc)?;
    Ok(compose(&before, &after))
}

/// Opens the Optimize tab of the Edit view.
fn open_optimize_tab(sc: &mut Sc<'_, '_>) -> Result<(), String> {
    sc.open_in_editor(FVS)?;
    sc.set_inspector_tab(2);
    pump(500);
    Ok(())
}

/// `optimizer_1.png`: the Optimize tab with the Objective list open.
///
/// # Errors
///
/// The design does not open or the list does not open.
pub fn optimizer_1(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    open_optimize_tab(sc)?;
    let combo = sc
        .view()
        .try_find("Optimize Objective")
        .ok_or_else(|| "the Optimize tab shows no Objective list".to_owned())?;
    press_on(sc.ctx.app.ui().window(), &combo);
    pump(600);
    let ui = sc.ui();
    let model = ui.global::<OptimizeModel>();
    let labels: Vec<String> = model
        .get_preset_labels()
        .iter()
        .chain(model.get_simple_preset_labels().iter())
        .map(String::from)
        .collect();
    let view = sc.view();
    let inspector = rect_of(&view, "EditorInspector")?;
    let mut area = inspector;
    let mut items = 0_u32;
    for label in &labels {
        for item in view.by_label(label) {
            let rect = crate::kit::Rect::of(&item);
            if rect.w > 0.0 && rect.h > 0.0 {
                area = area.union(rect);
                items += 1;
            }
        }
    }
    if items == 0 {
        return Err("the Objective list did not show its entries".to_owned());
    }
    let shot = sc.snap_main()?;
    Ok(area.grow(6.0).crop(&shot))
}

/// Runs Optimize on the open design (16 starts, 5 candidates) and waits for the result.
///
/// # Errors
///
/// Optimize is not available, reports a problem or does not finish.
pub fn run_optimize(sc: &mut Sc<'_, '_>) -> Result<(), String> {
    open_optimize_tab(sc)?;
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    let model = ui.global::<OptimizeModel>();
    model.set_starts_text("16".into());
    model.set_candidates(5);
    model.invoke_settings_edited();
    pump(300);
    if !Sc::wait(60, || editor.get_optimize_available()) {
        return Err(format!(
            "Optimize is not available: {}",
            editor.get_optimize_hint()
        ));
    }
    editor.invoke_optimize(
        editor.get_optimize_weight_windowing(),
        editor.get_optimize_weight_extinction(),
        editor.get_optimize_weight_tilt_brilliance(),
        editor.get_optimize_weight_yield(),
        editor.get_optimize_weight_tone(),
    );
    pump(500);
    let done = Sc::wait(1200, || {
        !editor.get_optimize_running() && model.get_has_results()
    });
    if !done {
        return Err(format!(
            "Optimize did not deliver candidates within 20 minutes: {}",
            editor.get_optimize_status()
        ));
    }
    pump(800);
    Ok(())
}

/// Picks the best candidate of the last run.
pub fn pick_best_candidate(sc: &Sc<'_, '_>) {
    let ui = sc.ui();
    ui.global::<OptimizeModel>().invoke_select_candidate(0);
    pump(1000);
}

/// `optimizer_2.png`: the Optimize tab after a run, the best candidate picked.
///
/// # Errors
///
/// Optimize does not deliver.
pub fn optimizer_2(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    run_optimize(sc)?;
    pick_best_candidate(sc);
    sc.expand_inspector();
    // The candidate list is below the run settings: scroll the inspector down to it.
    if let Some(inspector) = find_inherits(&sc.view(), "EditorInspector") {
        let area = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        area.as_ref()
            .unwrap_or(&inspector)
            .scroll(0.0, -OPTIMIZER_SCROLL);
        pump(500);
    }
    let shot = sc.snap_main()?;
    Ok(rect_of(&sc.view(), "EditorInspector")?.crop(&shot))
}
