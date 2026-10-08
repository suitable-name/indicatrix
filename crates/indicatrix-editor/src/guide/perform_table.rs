//! The recipes `perform` cannot derive from a step's goal, listed by guide id and step title.
//!
//! A goal like "Material is Diamond" says all there is to do; a goal like "tier G1 meets an exact
//! scale value" does not say which value, and a `Check` goal is only a test. Those steps name
//! their recipe here, written from the step's own action text: the same values the learner is
//! told to type. A step listed as [`Action::SkipOnly`] is one that cannot be done for the learner
//! (a drag, a tick box, a dialog of the operating system); it keeps its Skip step button even
//! where its goal would have implied a recipe. A step that is not listed gets the recipe its goal
//! implies, if any.
//!
//! The titles are matched against the guides when they are assembled, and a test fails when an
//! entry names a step that does not exist, so a renamed step cannot silently lose its recipe.

mod tiers;

use super::{Perform, WORKED_EXAMPLE_ID, lesson_start_spec};

/// The eight main positions of a ring on a 96-tooth, 8-fold gear, as the Indices field shows them.
pub(super) const MAIN_RING: &str = "0, 12, 24, 36, 48, 60, 72, 84";

/// What a listed step does.
#[derive(Clone, Debug)]
pub(super) enum Action {
    /// Next performs this.
    Do(Perform),
    /// The step has no automatic action.
    SkipOnly,
}

/// One listed step.
#[derive(Clone, Debug)]
pub(super) struct TableEntry {
    /// The guide's id.
    pub guide: &'static str,
    /// The step's title.
    pub step: &'static str,
    /// What Next does there.
    pub action: Action,
}

/// A step Next performs.
pub(super) fn recipe(guide: &'static str, step: &'static str, perform: Perform) -> TableEntry {
    TableEntry {
        guide,
        step,
        action: Action::Do(perform),
    }
}

/// A step Next cannot perform.
pub(super) const fn skip(guide: &'static str, step: &'static str) -> TableEntry {
    TableEntry {
        guide,
        step,
        action: Action::SkipOnly,
    }
}

/// Every listed step: the worked example's, then the tiers tutorials'.
pub(super) fn entries() -> Vec<TableEntry> {
    let mut entries = worked_example();
    entries.extend(tiers::entries());
    entries
}

/// The worked example: the values its steps tell the learner to type.
fn worked_example() -> Vec<TableEntry> {
    let id = WORKED_EXAMPLE_ID;
    vec![
        recipe(
            id,
            "Start a new design",
            Perform::NewDesign {
                template_index: 0,
                spec: Box::new(lesson_start_spec(0)),
            },
        ),
        recipe(
            id,
            "Add the girdle first",
            Perform::add_tier("90.0", 2, "1.0", "G1", MAIN_RING),
        ),
        recipe(
            id,
            "Add the pavilion main facets",
            Perform::add_tier("-40.0", 2, "0.56", "P1", MAIN_RING),
        ),
        recipe(
            id,
            "Add the crown main facets",
            Perform::add_tier("34.5", 2, "0.70", "C1", MAIN_RING),
        ),
        recipe(
            id,
            "Add the table",
            Perform::add_tier("0.0", 2, "0.46", "T", ""),
        ),
        // Optional, and its girdle diameter is the learner's own stone: no number to invent.
        skip(id, "Yield and rendering (optional)"),
    ]
}
