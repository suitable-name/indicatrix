//! Guided tutorials: the data model of a [`Guide`], its goals and the registry of guides.
//!
//! This covers the steps of a guide, the goals that complete a step, the worked example's
//! static step content ([`steps`]) and the completion predicates behind its automatic
//! advance ([`goal_reached`]).
//!
//! # Modules
//!
//! - [`steps`]: the worked example's ten static steps, shared with the web app (which
//!   indexes [`STEPS`] and pins its wording against it), and the lock [`Group`]s.
//! - `model`: [`Guide`], [`GuideStep`], [`GuideCategory`], [`StartingState`].
//! - `goal`: [`Goal`] (what completes a step) and the pure [`goal_met`].
//! - `catalog`: the built-in guides ([`worked_example_guide`], the welcome tour), the
//!   [`GuideCatalog`] that also holds guides generated at run time, and the lists of valid
//!   [`HIGHLIGHT_TARGETS`] and [`EVENTS`].
//! - `start`: what a [`StartingState`] asks of the editor ([`plan_start`], with the new
//!   design a lesson is cut from, [`lesson_start_spec`]) and how a UI waits for a design it
//!   asked for ([`launch_status`]).
//! - `listing`: the tutorial browser's rows, search and "Done" bookkeeping.
//! - `build_this_design`: [`build_this_design_guide`], the lesson that rebuilds a library
//!   design from an empty one (with `build_text`, its wording and number formats, and
//!   `rebuilt`, the [`Goal::DesignRebuilt`] comparison its last step uses).
//!
//! A UI owns navigation (start/next/back/close, the short "Done" moment before an
//! automatic advance) and every control lock, driven by the current step's allowed
//! groups. This crate owns the content and decides, from STATE, whether the current
//! step's goal is met -- never from which button was clicked, so every route to a goal
//! counts (quick add, inline edit, undo/redo, auto-solve), and a failed validation (which
//! never changes the design) can never advance.

mod build_text;
mod build_this_design;
mod catalog;
mod goal;
mod listing;
mod model;
mod rebuilt;
mod start;
pub mod steps;
mod tour;
mod tutorials;

#[cfg(test)]
mod build_this_design_tests;
#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod goal_tests;
#[cfg(test)]
mod rebuilt_tests;
#[cfg(test)]
mod tests;

pub use build_this_design::{
    BUILD_TITLE_PREFIX, BuildGuideError, BuildPlan, COMPARE_STEP_TITLE, LARGE_DESIGN_TIERS,
    MAX_TIERS_PER_STEP, REBUILD_ANGLE_TOL_DEG, REBUILD_MAST_REL_TOL, ReferenceDesign,
    SOLVE_STEP_TITLE, START_STEP_TITLE, TierRecipe, build_guide_id, build_this_design_guide,
    build_this_design_plan, is_build_guide_id, is_compare_step, original_label, reference_design,
};
pub use catalog::{
    EVENTS, GuideCatalog, HIGHLIGHT_TARGETS, WORKED_EXAMPLE_ID, goal_from_completion,
    static_guides, worked_example_guide,
};
pub use goal::{Goal, GoalContext, MeetKind, goal_met};
pub use listing::{
    BrowserRow, browser_rows, completed_count, matches_terms, progress_text, query_terms,
    record_completion, start_label, steps_text,
};
pub use model::{
    BUILD_ID_PREFIX, Guide, GuideCategory, GuideStep, StartingState, is_valid_guide_id,
};
pub use rebuilt::{MAST_FLOOR, mast_within};
pub use start::{
    EMPTY_START_PREFORM, LIBRARY_GRACE_TICKS, LaunchObservation, LaunchStatus, NEEDS_A_DESIGN,
    StartPlan, launch_status, lesson_start_spec, plan_start,
};
pub use steps::{ALL_GROUPS, EIGHT_FOLD_INDICES, Group, STEPS, Step};
pub use tour::{WELCOME_TOUR_ID, welcome_tour_guide};
pub use tutorials::{TIERS_MULTI_SELECTED, solving_events, viewing_events};

use indicatrix_cut_core::Design;

/// The completion key of a reading step: it never completes from state, only
/// through its own Next/Finish button.
pub const MANUAL: &str = "manual";

/// Completion key reported by the New Design action itself (an event, not a state
/// [`goal_reached`] could read back afterwards).
pub const NEW_DESIGN_CREATED: &str = "new_design_created";

/// How far a tier's angle may sit from the step's number and still count: the
/// form rounds nothing, so this only absorbs float noise and a trailing digit.
const ANGLE_TOLERANCE_DEG: f64 = 0.05;

/// How far an index may sit from the expected gear position and still count.
const INDEX_TOLERANCE: f64 = 1e-6;

/// Whether `design` (plus `solved_closed`: the editor's own solve verdict is
/// "solved", i.e. a closed solid) meets the goal named by completion key `key`.
///
/// `false` for every key that is not a state goal -- [`MANUAL`], and
/// [`NEW_DESIGN_CREATED`], which a UI reports as an event.
#[must_use]
pub fn goal_reached(key: &str, design: &Design, solved_closed: bool) -> bool {
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

/// The completion key to report through the UI's `notify` for the guide's step
/// `step_index`, or `None` when its goal is not reached (or the index is past the end).
///
/// The one place both apps turn "the design as it is now" into "this step is done":
/// they call it after every change to the design or its solve, with `solved_closed`
/// the editor's own verdict (a "solved" state that is not a problem).
#[must_use]
pub fn reached_completion(
    step_index: usize,
    design: &Design,
    solved_closed: bool,
) -> Option<&'static str> {
    let step = STEPS.get(step_index)?;
    goal_reached(step.completion, design, solved_closed).then_some(step.completion)
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

/// Whether `actual` and `expected` are the same gear positions, `gear` teeth
/// around (so 96 reads as 0 on a 96-tooth gear).
///
/// An empty `expected` list -- "leave
/// Indices blank" -- also accepts the single index 0, which is what a blank field
/// means.
#[must_use]
pub fn same_index_set(actual: &[f64], expected: &[f64], gear: f64) -> bool {
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
