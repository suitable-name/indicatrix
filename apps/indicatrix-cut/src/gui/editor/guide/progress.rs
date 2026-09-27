//! Automatic advance: evaluating the CURRENT guide step's goal against the design
//! and reporting it through `GuideModel.notify`.
//!
//! [`check_progress`] is the one entry point, called wherever the design or its
//! solve state can have changed: the end of `view::refresh_all` (New, Load, Open,
//! an explicit Solve), `view::refresh_editor_panel_stale` (every ordinary edit --
//! the tier form, quick add, inline edits, undo/redo, material and yield applies),
//! the tier-save/material/yield success callbacks themselves, and
//! `GuideModel.step_entered`. The background auto-solve's completion
//! (`auto_solve::apply`) and a matching solid-preview frame
//! (`view::push_solved_preview`) hold only a design snapshot, not the
//! `EditorState`, so they call [`check_design_progress`] -- the same evaluation,
//! one layer down. Every caller runs it AFTER `EditorModel.solve_state`/
//! `status_is_problem` have been updated for the result it is applying, since the
//! `solved_closed` goal reads them.
//!
//! Goals are judged from STATE, never from which button was clicked, so every
//! route to a goal counts (quick add, inline edit, undo/redo, auto-solve), and a
//! failed validation -- which never changes the design -- can never advance.

use super::{
    NEW_DESIGN_CREATED,
    steps::{EIGHT_FOLD_INDICES, STEPS},
};
use crate::{EditorModel, GuideModel, MainWindow, gui::editor::state::EditorState};
use indicatrix_cut_core::Design;
use slint::ComponentHandle;

/// How far a tier's angle may sit from the step's number and still count: the
/// form rounds nothing, so this only absorbs float noise and a trailing digit.
const ANGLE_TOLERANCE_DEG: f64 = 0.05;

/// How far an index may sit from the expected gear position and still count.
const INDEX_TOLERANCE: f64 = 1e-6;

/// Reports `key` to `GuideModel.notify` directly -- for a goal that is an EVENT
/// rather than a state [`goal_reached`] could read back afterwards (only
/// [`NEW_DESIGN_CREATED`] today). The Slint side ignores a key that is not the
/// current step's own.
pub(in crate::gui::editor) fn notify(ui: &MainWindow, key: &str) {
    ui.global::<GuideModel>().invoke_notify(key.into());
}

/// Evaluates the current guide step's goal against `state` and reports it to
/// `GuideModel.notify` when reached. A no-op while the guide is closed, on a manual
/// step, or while the current step is already showing "Done".
pub(in crate::gui::editor) fn check_progress(ui: &MainWindow, state: &EditorState) {
    check_design_progress(ui, &state.design);
}

/// [`check_progress`] for a caller holding only a `Design` (the background
/// auto-solve's completion, whose snapshot is the design it just solved).
pub(in crate::gui::editor) fn check_design_progress(ui: &MainWindow, design: &Design) {
    let guide = ui.global::<GuideModel>();
    if !guide.get_open() || guide.get_step_done() {
        return;
    }
    let Some(step) = usize::try_from(guide.get_step_index())
        .ok()
        .and_then(|index| STEPS.get(index))
    else {
        return;
    };
    let model = ui.global::<EditorModel>();
    let solved_closed = model.get_solve_state() == "solved" && !model.get_status_is_problem();
    if goal_reached(step.completion, design, solved_closed) {
        guide.invoke_notify(step.completion.into());
    }
}

/// Whether `design` (plus `solved_closed`: the Edit tab's own solve verdict is
/// "solved", i.e. a closed solid) meets the goal named by completion key `key`.
/// `false` for every key that is not a state goal -- the manual key, and
/// [`NEW_DESIGN_CREATED`], which is reported as an event through [`notify`].
pub(super) fn goal_reached(key: &str, design: &Design, solved_closed: bool) -> bool {
    match key {
        "material:Diamond" => design
            .material
            .name
            .as_deref()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("Diamond")),
        // A zero-tier design "solves" to its bare preform -- not the stone the
        // walkthrough is asking for.
        "solved_closed" => solved_closed && !design.tiers.is_empty(),
        "yield_applied" => design.girdle_diameter_mm.is_some(),
        NEW_DESIGN_CREATED => false,
        _ => key
            .strip_prefix("tier_named:")
            .is_some_and(|name| has_expected_tier(design, name)),
    }
}

/// The angle and index list the walkthrough asks for when naming tier `name`, or
/// `None` for a name no step uses.
fn expected_tier(name: &str) -> Option<(f64, &'static [f64])> {
    match name {
        "G1" => Some((90.0, &EIGHT_FOLD_INDICES)),
        "P1" => Some((-40.0, &EIGHT_FOLD_INDICES)),
        "C1" => Some((34.5, &EIGHT_FOLD_INDICES)),
        "T" => Some((0.0, &[])),
        _ => None,
    }
}

/// Whether `design` has a tier named `name` at that step's angle (within
/// [`ANGLE_TOLERANCE_DEG`]) with exactly that step's indices.
fn has_expected_tier(design: &Design, name: &str) -> bool {
    let Some((angle, indices)) = expected_tier(name) else {
        return false;
    };
    let gear = f64::from(design.meta.gear_teeth_abs());
    design.tiers.iter().any(|tier| {
        tier.names()
            .into_iter()
            .any(|n| n.trim().eq_ignore_ascii_case(name))
            && (tier.angle_deg - angle).abs() <= ANGLE_TOLERANCE_DEG
            && same_index_set(&tier.indices, indices, gear)
    })
}

/// Whether `a` and `b` are the same gear positions, `gear` teeth around (so 96
/// reads as 0 on a 96-tooth gear). An empty `expected` list -- "leave Indices
/// blank" -- also accepts the single index 0, which is what a blank field means.
pub(super) fn same_index_set(actual: &[f64], expected: &[f64], gear: f64) -> bool {
    let same = |a: f64, b: f64| {
        if gear > 0.0 {
            let d = (a - b).rem_euclid(gear);
            d.min(gear - d) <= INDEX_TOLERANCE
        } else {
            (a - b).abs() <= INDEX_TOLERANCE
        }
    };
    if expected.is_empty() {
        return match actual {
            [] => true,
            [only] => same(*only, 0.0),
            _ => false,
        };
    }
    actual.len() == expected.len()
        && expected.iter().all(|&e| actual.iter().any(|&a| same(a, e)))
        && actual.iter().all(|&a| expected.iter().any(|&e| same(a, e)))
}
