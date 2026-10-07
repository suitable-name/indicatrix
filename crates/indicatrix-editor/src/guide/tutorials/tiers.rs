//! The tiers tutorials: one guided lesson for each function that adds, edits, arranges or
//! removes tiers.
//!
//! - `basics`: add a tier, edit a tier, the index shorthands, the facet chips, quick add, and
//!   the inline angle cell with its nudging.
//! - `meets`: named facets, millimetre targets, adopting imported meets, pinning to the mast.
//! - `series`: the step ladder (plain and linked), mirroring to the other block, relations
//!   between tiers, and arithmetic in the number fields.
//! - `editing`: cheater offsets, notes, duplicate, move, delete and multi-select.
//! - `concave`: the concave tools.
//!
//! Every lesson starts from a design it can rely on (an empty one or a teaching template, whose
//! tier names are fixed), judges each step from the design as it is now (`Goal::tier`, or a
//! `Goal::Check` for the few states the plain goals cannot say), and locks everything the step
//! does not need except Undo. The tests in `tiers_tests` play every lesson in a real
//! `EditorSession`: each step's goal is false before the learner's edit and true after it.

mod basics;
mod concave;
mod editing;
mod meets;
mod series;

use crate::guide::{EIGHT_FOLD_INDICES, Goal, GoalContext, Group, Guide};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design};

/// The eight positions of a plain ring on a 96-tooth, 8-fold gear: one every 12 teeth.
const MAIN: [f64; 8] = EIGHT_FOLD_INDICES;

/// The ring half way between the mains.
const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

/// The tier form, the tier table and Undo: a lesson that edits tiers in the Tier tab.
const FORM: &[Group] = &[Group::TierForm, Group::TierTable, Group::History];

/// [`FORM`] with the Advanced controls (the millimetre Meets kinds, Rotate, Detach, the cheater
/// offset), which the Simple interface hides.
const FORM_ADVANCED: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Advanced,
    Group::History,
];

/// The tier table and Undo: a lesson that works only with the table's own buttons and cells.
const TABLE: &[Group] = &[Group::TierTable, Group::History];

/// [`TABLE`] with the Advanced controls (Steps / Mirror, Adopt, Pin, Detach).
const TABLE_ADVANCED: &[Group] = &[Group::TierTable, Group::Advanced, Group::History];

/// Every tiers tutorial, in the order the browser lists them.
#[must_use]
pub fn guides() -> Vec<Guide> {
    let mut guides = Vec::new();
    guides.extend(basics::guides());
    guides.extend(meets::guides());
    guides.extend(series::guides());
    guides.extend(editing::guides());
    guides.extend(concave::guides());
    guides
}

/// A goal that holds when `test` does. `label` says in a few words what it looks for.
const fn check(label: &'static str, test: fn(&GoalContext<'_>) -> bool) -> Goal {
    Goal::Check { label, test }
}

/// The row of the tier called `name` (case and surrounding spaces do not matter).
fn position(design: &Design, name: &str) -> Option<usize> {
    design.tiers.iter().position(|tier| {
        tier.names()
            .iter()
            .any(|known| known.trim().eq_ignore_ascii_case(name.trim()))
    })
}

/// The tier called `name`.
fn tier_called<'a>(design: &'a Design, name: &str) -> Option<&'a ConstraintTier> {
    position(design, name).and_then(|at| design.tiers.get(at))
}

/// Whether the tier called `tier` meets the facet called `target` by name.
fn meets_named(design: &Design, tier: &str, target: &str) -> bool {
    tier_called(design, tier).is_some_and(|found| {
        matches!(
            &found.constraint,
            MeetConstraint::MeetNamed(names)
                if names.iter().any(|name| name.trim().eq_ignore_ascii_case(target.trim()))
        )
    })
}

/// Whether the angle of the tier called `name` follows a relation.
fn is_driven(design: &Design, name: &str) -> bool {
    position(design, name).is_some_and(|at| design.is_tier_driven(at))
}

/// Whether tier `name` has exactly the positions `wanted` as its detached facets.
fn detached_is(design: &Design, name: &str, wanted: &[f64]) -> bool {
    tier_called(design, name).is_some_and(|found| {
        found.detached.len() == wanted.len()
            && wanted
                .iter()
                .all(|want| found.detached.iter().any(|have| (have - want).abs() < 1e-6))
    })
}
