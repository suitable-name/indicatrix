//! manufacturability checks -- the point where the editor starts
//! protecting the user from a design that solves geometrically but cannot
//! actually be cut.
//!
//! Four checks, every one reported as a warning **on** the design, never as
//! an edit that silently "fixes" anything:
//!
//! 1. [`mesh_checks::check_vanishing_facets`]: a later cut erased an earlier facet entirely.
//! 2. [`mesh_checks::check_undersized_facets`]: a facet survives but is too small to
//!    reliably cut and polish.
//! 3. [`authored_checks::check_gear_quantization`]: an authored index-wheel position does not
//!    land on a real gear tooth.
//! 4. [`authored_checks::check_cut_order`]: a tier's stated meet target comes later in the
//!    schedule than the tier itself -- geometrically solvable (the solver
//!    doesn't care about file order, see `indicatrix::geometry::meet_solver`'s
//!    module docs) but physically uncuttable in that order.
//!
//! [`check_manufacturability`] runs all four together over an
//! **already-solved** [`crate::design::Design`] (see its own doc comment for why that
//! matters) and returns every [`ManufacturabilityWarning`] found, in a fixed,
//! deterministic order (checks 1-4, tier order within each) -- never a
//! `HashMap`/`HashSet` in the decision path, matching this crate's
//! determinism contract.
//!
//! # Why these checks don't need a second solve
//!
//! `Design::planes`/`Design::status` each call `Design::solve` internally, and a
//! design with real meet-derived structure costs 5-6 seconds to solve. An editor
//! that just ran "Solve" already has a `Vec<SolvedTier>` in hand;
//! [`check_manufacturability`] takes that directly and builds the plane arrangement
//! via `Design::planes_from_solved` (no second solve), then meshes it via
//! [`indicatrix::geometry::stone_metrics::build_solid_mesh`] -- measured at 0.616ms
//! for a 103-tier design, negligible next to the solve that already happened.
//! Checks 3 and 4 need no mast at all (they read authored angle/index/constraint
//! state directly), so they run even when the design has never been solved.
//!
//! # Module layout
//!
//! [`warning`] is the one [`ManufacturabilityWarning`] type every check reports
//! through; [`mesh_checks`] is [`check_manufacturability`] itself plus checks 1-2
//! (the mesh-dependent pair); [`authored_checks`] is checks 3-4 (the mast-free
//! pair).

mod authored_checks;
mod mesh_checks;
#[cfg(test)]
mod tests;
mod warning;

pub use authored_checks::{check_cut_order, check_gear_quantization};
pub use mesh_checks::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability};
pub use warning::ManufacturabilityWarning;
