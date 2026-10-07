//! The solving, checking, optimizing and comparing tutorials: one guided lesson for each function
//! that solves a design, judges it, improves it or compares it with another.
//!
//! - `solve`: Solve, auto-solve, reading the solver status (and Abandon), the overall verdict
//!   and its Fix buttons, the Preform tab's proportions, and the yield and carat weight.
//! - `optimize`: Deep Solve, the Optimize tab, Retarget for a new material (Shift and Optimize)
//!   and the angle sweep.
//! - `compare`: the History panel and jumping, variants, Snapshot and Compare, and Edit as Text
//!   with its comparison.
//!
//! Every lesson starts from a design it can rely on (a teaching template, whose tier names are
//! fixed, or the design that is open), judges each step from the design as it is now (a tier, the
//! material, the preform, the girdle size, the inspector tab, the solve verdict), and waits for a
//! UI event only for what leaves no trace in the design (the Solve button was pressed, a result
//! list appeared, a snapshot was taken). A dialog covers the lesson's own panel while it is open,
//! so a step that sends the learner into a dialog says everything the dialog needs and waits for
//! its outcome in the design (a retarget applied, a sweep angle used, text applied) or for one
//! event, and the next step is read once the dialog is closed.
//!
//! Template designs start without a material, so every lesson whose figures need one begins with
//! a "choose the material" step. Lessons that must not be hurried along by auto-solve switch it off
//! first, and a reading step at the end puts it back.
//!
//! The tests in `solving_tests` play every lesson in a real `EditorSession`: each step's goal is
//! false before the learner's action and true after it, and the premises a lesson states about
//! the design (a girdle with no anchor does not solve, a Fix can be planned) are asserted too.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

mod compare;
mod optimize;
mod solve;

use super::solving_events as events;
use crate::guide::{Goal, GoalContext, Group, Guide};
use indicatrix_cut_core::{ConstraintTier, Design};

/// The session template index of the Standard Round Brilliant (card 1 of the New Design dialog):
/// eight tiers, a table, star and break rings, mains and a culet.
const STANDARD: usize = 1;

/// The session template index of the Rich Teaching Design (card 5): a table, a crown main ring, a
/// girdle and a pavilion main ring, every depth pinned.
const RICH: usize = 5;

/// The crown main angle of the Rich Teaching Design.
const RICH_CROWN: f64 = 34.5;

/// The pavilion main angle of the Rich Teaching Design.
const RICH_PAVILION: f64 = -40.0;

/// How close an angle must be to a wanted one to count (the tier goals' own tolerance).
const ANGLE_TOLERANCE: f64 = 0.05;

/// Solve, the auto-solve list and Undo: a lesson step that only presses Solve or reads.
const SOLVE: &[Group] = &[Group::Solve, Group::History];

/// The tier form, the tier table and Undo: a step that edits tiers.
const EDIT: &[Group] = &[Group::TierForm, Group::TierTable, Group::History];

/// [`EDIT`] with Solve and its auto-solve list: a step that edits a tier and then solves.
const EDIT_SOLVE: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Solve,
    Group::History,
];

/// [`EDIT`] with the Advanced controls, which the Simple interface hides (the Meets kinds).
const EDIT_ADVANCED: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Advanced,
    Group::History,
];

/// [`EDIT_ADVANCED`] with Solve: a step that edits the Meets setting and then solves.
const EDIT_ADVANCED_SOLVE: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Advanced,
    Group::Solve,
    Group::History,
];

/// The tier table, Solve and Undo: the verdict's Fix buttons need the tier table unlocked.
const FIX: &[Group] = &[Group::TierTable, Group::Solve, Group::History];

/// Design Settings (the material), Solve and Undo.
const SETTINGS: &[Group] = &[Group::DesignSettings, Group::Solve, Group::History];

/// The Preform tab, Solve and Undo.
const PREFORM: &[Group] = &[Group::PreformTab, Group::Solve, Group::History];

/// The advanced tools (Deep Solve, Optimize, Retarget, Snapshot, Compare, Edit as Text, Angle
/// Sweep), Solve and Undo.
const TOOLS: &[Group] = &[Group::Advanced, Group::Solve, Group::History];

/// [`TOOLS`] with the tier form, for a step that changes a tier between two tool uses.
const TOOLS_EDIT: &[Group] = &[
    Group::Advanced,
    Group::TierForm,
    Group::TierTable,
    Group::Solve,
    Group::History,
];

/// Only Undo, Redo and the History tab: a step that goes through the history.
const HISTORY: &[Group] = &[Group::History];

/// [`HISTORY`] with the tier form: a step that edits a tier and then looks at the history.
const HISTORY_EDIT: &[Group] = &[Group::TierForm, Group::TierTable, Group::History];

/// Every solving tutorial, in the order the browser lists them.
#[must_use]
pub fn guides() -> Vec<Guide> {
    let mut guides = Vec::new();
    guides.extend(solve::guides());
    guides.extend(optimize::guides());
    guides.extend(compare::guides());
    guides
}

/// A goal that waits for the UI event `name`.
fn event(name: &str) -> Goal {
    Goal::Event(name.to_owned())
}

/// A goal met by pressing Solve (the event) when the stone then closes (the verdict).
fn solved_by_button() -> Goal {
    Goal::All(vec![event(events::SOLVE_REQUESTED), Goal::SolvedClosed])
}

/// A goal that holds when `test` does. `label` says in a few words what it looks for.
const fn check(label: &'static str, test: fn(&GoalContext<'_>) -> bool) -> Goal {
    Goal::Check { label, test }
}

/// The tier called `name` (case and surrounding spaces do not matter).
fn tier_called<'a>(design: &'a Design, name: &str) -> Option<&'a ConstraintTier> {
    design.tiers.iter().find(|tier| {
        tier.names()
            .iter()
            .any(|known| known.trim().eq_ignore_ascii_case(name.trim()))
    })
}

/// Whether the tier called `name` is at `wanted` degrees.
fn tier_at(design: &Design, name: &str, wanted: f64) -> bool {
    tier_called(design, name).is_some_and(|tier| (tier.angle_deg - wanted).abs() <= ANGLE_TOLERANCE)
}

/// Whether the Rich Teaching Design's crown and pavilion mains are as the template made them.
fn rich_at_start(ctx: &GoalContext<'_>) -> bool {
    tier_at(ctx.design, "Crown Main", RICH_CROWN)
        && tier_at(ctx.design, "Pavilion Main", RICH_PAVILION)
}
