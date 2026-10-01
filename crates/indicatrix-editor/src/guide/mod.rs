//! The worked-example walkthrough: its step content ([`steps`]) and the completion
//! predicates behind its automatic advance ([`goal_reached`]).
//!
//! A UI owns navigation (start/next/back/close, the short "Done" moment before an
//! automatic advance) and every control lock, driven by the current step's
//! [`steps::Step::allow`] groups. This module owns the content and decides, from
//! STATE, whether the current step's goal is met -- never from which button was
//! clicked, so every route to a goal counts (quick add, inline edit, undo/redo,
//! auto-solve), and a failed validation (which never changes the design) can never
//! advance.

pub mod steps;
#[cfg(test)]
mod tests;

pub use steps::{ALL_GROUPS, EIGHT_FOLD_INDICES, Group, STEPS, Step};

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
