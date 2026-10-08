//! Next on an unfinished step: carries out the step's recipe
//! ([`indicatrix_editor::guide::Perform`]) the way the learner would.
//!
//! Every part of a recipe is the very callback, property write or palette command the matching
//! control uses (the Tier form's Save, the Steps panel's Generate, the Solve button, the New
//! Design dialog's Create), so the edit is one undo step like the learner's own, a refused entry
//! is reported under its field by the same parser, and the guide's control locks do not matter:
//! they stop the learner's clicks, not the app. Nothing here judges success. The step's goal is
//! checked from state as ever ([`progress::check_now`]), so the step completes the usual way
//! (the "Done" chip, then the advance), and a recipe that did not get there leaves the step
//! open with a button that now says Skip step.
//!
//! The recipe runs a moment after the button press, on a later pass of the event loop, like a
//! palette command, never inside the button's own Slint handler. A recipe that starts a solve
//! has no result yet when it returns, so the button waits ([`GuideModel`]'s `perform_busy`) and
//! the overlay layer's timer calls [`tick`] a few times a second until the goal is met or the
//! recipe's ticks ([`SETTLE_TICKS`] for a solve or a new design, [`QUICK_TICKS`] otherwise) have
//! passed.

use super::{launch, progress, runtime};
use crate::{
    ConcaveFormData, EditorModel, GuideModel, MainWindow, RelationModel,
    gui::{commands, show_toast},
};
use indicatrix_cut_core::design::ConcaveTier;
use indicatrix_editor::{
    guide::{
        CMD_SOLVE, Perform, StepSeries, TierEntry, concave_position, resolve_tier, tier_position,
    },
    loading::{ConcaveTierFormFields, concave_tier_form_fields},
};
use slint::{ComponentHandle, Model as _};
use std::{cell::Cell, time::Duration};

/// How long after the press the recipe runs. Long enough for the press to finish, as the
/// palette's own delay is.
const RUN_DELAY: Duration = Duration::from_millis(20);

/// How many timer ticks (300 ms each) a recipe that starts a solve, or asks the unsaved-changes
/// question, may take to meet its goal before the button gives up waiting and offers Skip step.
/// A late result still completes the step by itself.
const SETTLE_TICKS: u32 = 10;

/// How many ticks any other recipe gets: its callbacks have finished by the time it returns, so
/// a goal that is not met after a moment will not be met by waiting.
const QUICK_TICKS: u32 = 2;

thread_local! {
    /// Timer ticks since the recipe ran.
    static TICKS: Cell<u32> = const { Cell::new(0) };
    /// How many ticks the running recipe may take.
    static TICK_LIMIT: Cell<u32> = const { Cell::new(QUICK_TICKS) };
}

/// Whether the result of `perform` arrives after it returns: a solve, or a new design that may
/// first ask about unsaved changes.
fn waits_for_result(perform: &Perform) -> bool {
    match perform {
        Perform::NewDesign { .. } => true,
        Perform::Command(id) => id == CMD_SOLVE,
        Perform::Sequence(parts) => parts.iter().any(waits_for_result),
        _ => false,
    }
}

/// `GuideModel.perform(index)`: runs the recipe of step `index` of the running guide.
pub(super) fn start(ui: &MainWindow, index: i32) {
    let model = ui.global::<GuideModel>();
    let guide_id = model.get_guide_id().to_string();
    let recipe = usize::try_from(index)
        .ok()
        .and_then(|at| runtime::step_perform(&guide_id, at));
    let Some(recipe) = recipe else {
        give_up(ui);
        return;
    };
    TICKS.set(0);
    TICK_LIMIT.set(if waits_for_result(&recipe) {
        SETTLE_TICKS
    } else {
        QUICK_TICKS
    });
    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(RUN_DELAY, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<GuideModel>();
        // The learner moved on (Back, close) in the meantime: the press no longer applies.
        if model.get_guide_id().as_str() != guide_id || model.get_step_index() != index {
            model.set_perform_busy(false);
            return;
        }
        if let Err(message) = execute(&ui, &recipe) {
            show_toast(&ui, &message, "error");
            give_up(&ui);
            return;
        }
        progress::check_now(&ui);
        settle_if_done(&ui);
    });
}

/// `GuideModel.perform_tick`: the recipe has run and the goal is not met yet.
pub(super) fn tick(ui: &MainWindow) {
    progress::check_now(ui);
    if settle_if_done(ui) {
        return;
    }
    let ticks = TICKS.get() + 1;
    TICKS.set(ticks);
    if ticks >= TICK_LIMIT.get() {
        give_up(ui);
    }
}

/// Stops waiting when the step is done. Returns whether it was.
fn settle_if_done(ui: &MainWindow) -> bool {
    let model = ui.global::<GuideModel>();
    let done = model.get_step_done();
    if done {
        model.set_perform_busy(false);
    }
    done
}

/// The recipe did not finish the step: the button stops waiting and offers Skip step.
fn give_up(ui: &MainWindow) {
    let model = ui.global::<GuideModel>();
    model.set_perform_busy(false);
    model.set_perform_failed(true);
}

/// Carries out `perform`.
///
/// # Errors
///
/// A sentence for the learner: the row a recipe names is not in the design, or a palette
/// command cannot run right now. A refused form entry is not an error here: it is reported
/// under its field by the form's own callback, and the step stays open.
fn execute(ui: &MainWindow, perform: &Perform) -> Result<(), String> {
    match perform {
        Perform::NewDesign {
            template_index,
            spec,
        } => {
            launch::request_new_design(ui, spec, *template_index);
            Ok(())
        }
        Perform::Tier(entry) => save_tier(ui, entry),
        Perform::ConcaveTier { edit, tier } => save_concave_tier(ui, edit.as_deref(), tier),
        Perform::SelectTier(name) => select_row(ui, name).map(|_| ()),
        Perform::Steps(series) => {
            generate_steps(ui, series);
            Ok(())
        }
        Perform::MirrorTier { tier, suffix } => {
            let row = select_row(ui, tier)?;
            ui.global::<EditorModel>()
                .invoke_mirror_tier_to_other_block(row, suffix.as_str().into());
            Ok(())
        }
        Perform::ClearRelation(tier) => {
            let row = select_row(ui, tier)?;
            ui.global::<RelationModel>().invoke_remove_relation(row);
            Ok(())
        }
        Perform::TierNote { tier, text } => {
            let row = select_row(ui, tier)?;
            ui.global::<EditorModel>()
                .invoke_apply_tier_note(row, text.as_str().into());
            Ok(())
        }
        Perform::CheaterOffset { tier, text } => {
            let row = select_row(ui, tier)?;
            ui.global::<EditorModel>()
                .invoke_apply_cheater_offset(row, text.as_str().into());
            Ok(())
        }
        Perform::Material(name) => apply_material(ui, name),
        Perform::Yield { girdle_diameter_mm } => {
            apply_yield(ui, *girdle_diameter_mm);
            Ok(())
        }
        Perform::ViewMode(mode) => {
            commands::set_solid_view_mode(ui, *mode);
            Ok(())
        }
        Perform::InspectorTab(tab) => {
            commands::show_inspector_tab(ui, *tab);
            Ok(())
        }
        Perform::Command(id) => commands::run_unlocked(ui, id),
        Perform::Sequence(parts) => parts.iter().try_for_each(|part| execute(ui, part)),
    }
}

/// Picks the row called `name` in the tier table (a flat tier, else a concave one) and returns
/// its table position.
fn select_row(ui: &MainWindow, name: &str) -> Result<i32, String> {
    let position = with_design(|design| {
        tier_position(design, name)
            .or_else(|| concave_position(design, name).map(|at| design.tiers.len() + at))
    })?
    .ok_or_else(|| format!("There is no tier called '{}'.", name.trim()))?;
    let row = i32::try_from(position).map_err(|_| "The design has too many tiers.".to_owned())?;
    ui.global::<EditorModel>().set_selected_tier_index(row);
    Ok(row)
}

/// Reads the open design.
fn with_design<R>(read: impl FnOnce(&indicatrix_cut_core::Design) -> R) -> Result<R, String> {
    let state = runtime::state().ok_or_else(|| "The editor is not ready yet.".to_owned())?;
    let st = state
        .try_borrow()
        .map_err(|_| "The editor is busy for a moment.".to_owned())?;
    Ok(read(&st.design))
}

/// The Tier form: "+ Add Tier" or the picked row, then Add Tier or Save Tier with the six
/// fields `entry` describes.
fn save_tier(ui: &MainWindow, entry: &TierEntry) -> Result<(), String> {
    let save = with_design(|design| resolve_tier(design, entry))??;
    if save.index < 0 {
        // The button that opens the form blank: the new tier goes to the end of the table.
        commands::add_tier(ui);
    } else {
        ui.global::<EditorModel>()
            .set_selected_tier_index(save.index);
    }
    ui.global::<EditorModel>().invoke_save_tier(
        save.index,
        save.angle.into(),
        save.constraint_kind,
        save.constraint_text.into(),
        save.name.into(),
        save.indices.into(),
    );
    Ok(())
}

/// The concave form's Add Concave Tier or Save Concave Tier with `tier` typed in.
fn save_concave_tier(
    ui: &MainWindow,
    edit: Option<&str>,
    tier: &ConcaveTier,
) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    let position = match edit {
        Some(name) => {
            let at = with_design(|design| {
                concave_position(design, name).map(|at| design.tiers.len() + at)
            })?
            .ok_or_else(|| format!("There is no concave tier called '{}'.", name.trim()))?;
            let row = i32::try_from(at).map_err(|_| "The design has too many tiers.".to_owned())?;
            editor.set_selected_tier_index(row);
            row
        }
        None => {
            editor.invoke_add_concave_tier();
            -1
        }
    };
    editor.invoke_save_concave_tier(position, concave_form_data(tier));
    Ok(())
}

/// The concave form's fields as the inspector's draft holds them.
fn concave_form_data(tier: &ConcaveTier) -> ConcaveFormData {
    let ConcaveTierFormFields {
        name,
        angle_deg,
        indices,
        instructions,
        tool,
        tool_azimuth_deg,
        x,
        y,
        z,
        diameter_ratio,
        tool_angle_deg,
        reciprocating,
    } = concave_tier_form_fields(tier);
    ConcaveFormData {
        name: name.into(),
        angle_deg: angle_deg.into(),
        indices: indices.into(),
        instructions: instructions.into(),
        tool: tool.into(),
        tool_azimuth_deg: tool_azimuth_deg.into(),
        x: x.into(),
        y: y.into(),
        z: z.into(),
        diameter_ratio: diameter_ratio.into(),
        tool_angle_deg: tool_angle_deg.into(),
        reciprocating,
    }
}

/// The Steps panel's Generate with `series` typed in.
fn generate_steps(ui: &MainWindow, series: &StepSeries) {
    if series.linked {
        ui.global::<RelationModel>()
            .invoke_generate_step_series_linked(
                series.name.as_str().into(),
                series.start.as_str().into(),
                series.step.as_str().into(),
                series.count,
                series.indices.as_str().into(),
                series.anchor.as_str().into(),
            );
    } else {
        ui.global::<EditorModel>().invoke_generate_step_series(
            series.name.as_str().into(),
            series.start.as_str().into(),
            series.step.as_str().into(),
            series.count,
            series.indices.as_str().into(),
            series.anchor.as_str().into(),
        );
    }
}

/// Design Settings: pick `name` in the Material list, then Apply Material.
fn apply_material(ui: &MainWindow, name: &str) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    let options = editor.get_material_combo_options();
    let wanted = name.trim();
    let index = (0..options.row_count())
        .find(|&at| {
            options
                .row_data(at)
                .is_some_and(|option| option.trim().eq_ignore_ascii_case(wanted))
        })
        .and_then(|at| i32::try_from(at).ok())
        .ok_or_else(|| format!("'{wanted}' is not in the Material list."))?;
    editor.set_material_combo_index(index);
    editor.invoke_apply_design_material(
        index,
        editor.get_ri_override_text(),
        editor.get_body_color_index(),
    );
    Ok(())
}

/// The Preform tab: type the Girdle Diameter, then Apply Yield Inputs.
fn apply_yield(ui: &MainWindow, girdle_diameter_mm: f64) {
    let editor = ui.global::<EditorModel>();
    let text = girdle_diameter_mm.to_string();
    editor.set_girdle_diameter_mm(text.as_str().into());
    editor.invoke_apply_yield_inputs(text.into(), 0, editor.get_specific_gravity_override());
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::design::{ConcaveTool, ToolMotion};

    #[test]
    fn only_a_solve_or_a_new_design_has_its_result_after_it_returns() {
        assert!(waits_for_result(&Perform::solve()));
        assert!(waits_for_result(
            &Perform::select("Table").then(Perform::solve())
        ));
        assert!(!waits_for_result(&Perform::undo()));
        assert!(!waits_for_result(&Perform::ViewMode(1)));
        assert!(!waits_for_result(&Perform::add_tier(
            "90", 2, "1", "G1", "0:12:96"
        )));
    }

    #[test]
    fn a_concave_tier_fills_the_inspectors_draft_with_the_forms_own_text() {
        let tier = ConcaveTier {
            name: "Groove".to_owned(),
            angle_deg: -42.0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            instructions: String::new(),
            tool: ConcaveTool::Cylinder,
            tool_azimuth_deg: 0.0,
            displacement: [0.0; 3],
            diameter_ratio: 0.25,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        };
        let data = concave_form_data(&tier);
        assert_eq!(data.name.as_str(), "Groove");
        assert_eq!(data.tool.as_str(), "CYL");
        assert!(data.reciprocating);
        // Saved as the form saves it, the draft makes the same tier back.
        let fields = ConcaveTierFormFields {
            name: data.name.to_string(),
            angle_deg: data.angle_deg.to_string(),
            indices: data.indices.to_string(),
            instructions: data.instructions.to_string(),
            tool: data.tool.to_string(),
            tool_azimuth_deg: data.tool_azimuth_deg.to_string(),
            x: data.x.to_string(),
            y: data.y.to_string(),
            z: data.z.to_string(),
            diameter_ratio: data.diameter_ratio.to_string(),
            tool_angle_deg: data.tool_angle_deg.to_string(),
            reciprocating: data.reciprocating,
        };
        let parsed = indicatrix_editor::loading::parse_concave_tier_form(&fields, 96)
            .expect("the draft parses");
        assert_eq!(parsed, tier);
    }
}
