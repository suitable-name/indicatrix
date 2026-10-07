//! The rough planner: how many stones of which designs fit one piece of rough,
//! and how to saw it.
//!
//! # The model
//!
//! The rough is a [`RoughModel`]: a base shape (block, cylinder or pebble, see
//! [`shape`]) with an ordered list of planar cuts (edge chamfers, corner cuts, flat
//! faces), always a convex polytope. It is sawn by staged guillotine cuts
//! (slabs across axis A, bars across axis B, pieces across axis C, in one of
//! the six [`CutOrder`]s), every cut costing one kerf. Each piece holds one
//! stone; the stone is the design scaled uniformly to the largest size that
//! fits the piece minus the per-side allowance, in any of the six axis
//! assignments (the table may face any rough face; the caliper footprint
//! already covers rotation about the table normal). A stone narrower than the
//! minimum width is infeasible. The objective is the total finished volume
//! (equivalently carats: one material, one specific gravity).
//!
//! The stone count is a MAXIMUM: a layout may hold any `n` in `1..=K`.
//!
//! # Entry point and paths
//!
//! [`plan()`](plan::plan) takes a [`PlanInput`] (model, settings, candidate designs and
//! their convex outlines) and dispatches on the model:
//!
//! - a plain uncut block goes through the block planner described below
//!   ([`plan_rough`] is its standalone form);
//! - every other model goes through [`shaped`]: the same guillotine DP, but pieces are
//!   classified against the rough's planes and scaled by a small LP, followed by shaped
//!   uniform grids and continuous refinement.
//!
//! Both paths add the exact single-stone fits of [`fit`]: each design's true convex outline
//! is rotated freely and scaled/translated by LP inside the rough ([`fit_single_stones`],
//! also available stage by stage as [`screen_designs`], [`shortlist`], [`fit_shortlisted`]
//! and [`merge_fits`] so callers can spread designs over lanes without changing the
//! result). The single-stone layouts join the ranked list.
//!
//! Inputs are checked first ([`PlanSettings::validate`], [`RoughBlock::validate`]): an
//! input that fails gives an empty result, never a panic or a non-finite total.
//!
//! # The pieces of the block planner
//!
//! - [`pareto_front`]: drops designs dominated in `(L/W, H/W, V/W^3)`.
//! - [`choose_grid`] / [`build_piece_table`]: the unit grid and the shared,
//!   read-only [`PieceTable`] (`Send + Sync`) of best stone value per piece
//!   size.
//! - [`plan_rough_for_order`]: the mixed DP of ONE cut order over the shared
//!   table, every stone count `n` in `1..=K` reconstructed as a candidate. The
//!   six orders are independent, so an application can run them on six threads
//!   sharing `&Grid`, `&PieceTable` and `&[CandidateDesign]`.
//! - [`plan_alternatives`]: the leave-one-out mixed alternatives (best mixed
//!   layout's most-used design removed from the full list, Pareto front
//!   recomputed, DP re-run, up to three times).
//! - [`uniform_layouts`]: every design in every fully filled `a x b x c`
//!   grid of equal cells.
//! - [`refine`] / [`finish_plan`]: continuous optimisation of the cut
//!   positions, then the final ranking ([`merge_and_rank`]).
//! - [`plan_rough`]: all of the above, sequentially.
//!
//! # Determinism and portability
//!
//! The block planner is single-threaded, with no I/O, no clock, no
//! `HashMap`/`HashSet`, no libm calls beyond IEEE `+ - * /`, `min`/`max`, `round` and
//! `mul_add`: identical input gives bitwise-identical output, and the module builds for
//! `wasm32-unknown-unknown`. The result never depends on the order of the
//! candidate list (it is sorted by `entry_id` first).

mod dp;
pub mod fit;
pub mod locate;
mod lp;
mod pareto;
mod piece;
pub mod plan;
mod rank;
mod refine;
pub mod shape;
pub mod shaped;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_assembly;
#[cfg(test)]
mod tests_brute;
#[cfg(test)]
mod tests_input;
#[cfg(test)]
mod tests_lp;
mod tree;
mod types;
mod uniform;

pub use dp::{plan_alternatives, plan_rough_for_order};
pub use fit::*;
pub use pareto::pareto_front;
pub use piece::{Grid, PieceTable, build_piece_table, choose_grid};
pub use plan::*;
pub use rank::{best_layout, flatten_groups, merge_and_rank, rank_indices};
pub use refine::{final_ranking, finish_plan, own_pool, refine};
pub use shape::{
    BoxFace, HullError, MAX_HULL_PLANES, MAX_MESH_TRIANGLES, MeshError, RoughBase, RoughCut,
    RoughMeasure, RoughMesh, RoughModel, ShapeError, import_hull, import_mesh,
};
pub use types::{
    Axis, BarCut, CandidateDesign, CutOrder, CutPlan, LayoutGroup, PlacedStone, PlanInputError,
    PlanProgress, PlanSettings, RoughBlock, RoughLayout, SlabCut,
};
pub use uniform::uniform_layouts;

/// Layouts in every final ranked list (the block planner's, the shaped planner's and the
/// refinement's top-up); the exact single-stone fit counts in [`plan`] derive from it.
pub const FINAL_TOP: usize = 10;
/// Candidates refined before the final cut (the best 20 of the ranked list).
pub const REFINE_TOP: usize = 20;
/// Designs the shaped uniform pass evaluates.
pub const SHAPED_UNIFORM_DESIGNS: usize = 64;
/// The one per-design-set limit of every ranked list: at most this many layouts may use the
/// same set of designs (counts ignored). A selection that uses a single design therefore
/// shows at most this many layouts, however many stone counts are feasible.
pub(crate) const SAME_SET_CAP: usize = 3;
/// Refinement passes over a layout's boundaries.
pub(crate) const PASSES: usize = 3;
/// Relative improvement below which the refinement passes stop.
pub(crate) const REL_TOL: f64 = 1e-9;
/// Upper bound on the summed DP operation estimate before the unit grid shrinks.
pub(crate) const OP_CAP: f64 = 3.0e9;
/// The value of an infeasible piece or an unreachable DP state.
pub(crate) const NEG: f64 = f64::NEG_INFINITY;

/// Plan the rough: up to 10 ranked layouts ([`FINAL_TOP`]).
///
/// Runs, in order, the Pareto prune, the grid and piece table, the six
/// cut-order DPs, the leave-one-out alternatives, the single-design pass, the
/// refinement of the best 20 and the final ranking. `on_progress` is called
/// throughout with a [`PlanProgress`]; returning `false` cancels.
///
/// The list holds distinct compositions only, and at most three layouts per design set
/// (the `SAME_SET_CAP` of the ranking): a selection with one design shows at most three,
/// however many stone counts are feasible, and any selection shows fewer than ten when fewer
/// distinct compositions survive that cap.
///
/// Returns `None` when cancelled and `Some(vec![])` when nothing is feasible
/// (invalid settings or rough, see [`PlanSettings::validate`] and [`RoughBlock::validate`];
/// no valid designs; or the rough is too small for a stone of the minimum width). The input
/// order of `designs` never changes the result.
pub fn plan_rough(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    if settings.validate().is_err() || rough.validate().is_err() {
        return Some(Vec::new());
    }
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }
    let clean = pareto::sanitize(designs);
    let front = pareto_front(&clean);
    if front.is_empty() {
        return Some(Vec::new());
    }
    let grid = choose_grid(rough, settings);
    let table = build_piece_table(&grid, &front, on_progress)?;
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(plan_rough_for_order(
            &grid,
            &table,
            &front,
            order,
            settings.count,
            on_progress,
        )?);
    }
    let alternatives = match rank::best_layout(&mixed) {
        Some(best) => plan_alternatives(&grid, &clean, best, settings.count, on_progress)?,
        None => Vec::new(),
    };
    let mut groups = vec![LayoutGroup {
        pool: front,
        layouts: mixed,
    }];
    groups.extend(alternatives);
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform_layouts(rough, settings, &clean, on_progress)?,
    });
    finish_plan(rough, settings, &clean, &groups, on_progress)
}
