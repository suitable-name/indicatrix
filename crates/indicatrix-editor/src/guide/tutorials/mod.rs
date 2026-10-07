//! The per-function tutorials: one guided lesson for each CAD function, written as plain
//! [`Guide`] data and registered with the catalogue through [`all`].
//!
//! One module per area, each with a `pub fn guides() -> Vec<Guide>`:
//!
//! - `tiers`: adding and editing tiers, meets and targets, step series, mirroring,
//!   relations, the concave tools, and duplicating, moving, deleting and multi-selecting.
//! - `solving`: solving and reading the result, the verdict and its Fix buttons, Deep Solve,
//!   Optimize, Retarget, the angle sweep, the History panel, variants, Snapshot and Compare,
//!   and Edit as Text.
//! - `viewing`: the Solid and Diagram views, the drag handles and Slice, Live Render and
//!   materials, cutting mode and the exports, Save and Open, the library and the Rough
//!   Planner, and the program's own preferences, palette, keyboard and help.
//!
//! Add an area by adding its `mod` line here and one `.extend(...)` call in [`all`]. The
//! catalogue (`catalog::static_guides`) appends [`all`] after its own built-in guides, and
//! the registry tests check every guide returned here (unique ids, known highlight targets
//! and events, steps that say what they wait for).

use super::model::Guide;

mod tiers;

#[cfg(test)]
mod tiers_tests;

mod solving;

#[cfg(test)]
mod solving_tests;

pub mod solving_events;

mod viewing;

#[cfg(test)]
mod viewing_tests;

pub mod viewing_events;

/// The UI event the tier table reports once two or more tiers are ticked.
///
/// The ticks form the multi-select group (Ctrl+click, Shift+click, the Select tick boxes or
/// Space). Selecting changes no design data, so a step cannot read it from state.
pub const TIERS_MULTI_SELECTED: &str = "tiers_multi_selected";

/// Every per-function tutorial, in browser order.
#[must_use]
pub fn all() -> Vec<Guide> {
    let mut guides = Vec::new();
    guides.extend(tiers::guides());
    guides.extend(solving::guides());
    guides.extend(viewing::guides());
    guides
}
