//! The rough planner: how many stones of which designs fit one piece of rough,
//! and how to saw it.
//!
//! # The model
//!
//! The rough is a [`RoughModel`]: a base shape (block, cylinder, pebble or an imported
//! hull or mesh, see [`shape`]) with an ordered list of planar cuts (edge chamfers, corner
//! cuts, flat faces). The planner works from the convex polytope of the rough's
//! half-spaces; a mesh rough's true surface, which may be non-convex, is asked only to
//! reject or shrink what reaches into air (`shape::RoughModel::mesh`). It is sawn by
//! staged guillotine cuts (slabs across axis A, bars across axis B, pieces across axis
//! C, in one of the six [`CutOrder`]s), every cut costing one kerf. Each piece holds one
//! stone; the stone is the design scaled uniformly to the largest size that
//! fits the piece minus the per-side allowance, in any of the six axis
//! assignments (the table may face any rough face; the caliper footprint
//! already covers rotation about the table normal). A stone narrower than the
//! minimum width is infeasible. The objective is the total finished volume
//! (equivalently carats: one material, one specific gravity).
//!
//! Two caps limit a layout. The number of DIFFERENT designs ([`PlanSettings::max_designs`]) is
//! always capped: the DPs may mix any number, and the design-cap chain ([`plan_design_cap`],
//! `shaped::plan_shaped_design_cap`) drops the least contributing design and solves again
//! until the best layout is within the cap. The number of stones in all
//! ([`PlanSettings::max_stones`]) is free by default (`0`): a layout holds as many as fit
//! best. A stone cap is part of the search, never a cut-down of a larger layout: the order
//! DPs solve for the best layout of at most that many stones ([`plan_rough_for_order`]; the
//! free optimum is used when it already fits, so a cap that does not bind changes nothing),
//! the single-design pass only enumerates grids within it, and the chain and the alternatives
//! work inside both caps. Every ranked list keeps only layouts within both
//! ([`rank_indices_within`]), and the refinement never re-picks a design past the design cap
//! ([`refine_capped`]) nor adds a stone.
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
//!   table, its best layout reconstructed as the order's candidate. The
//!   six orders are independent, so an application can run them on six threads
//!   sharing `&Grid`, `&PieceTable` and `&[CandidateDesign]`.
//! - [`plan_design_cap`]: the design-cap chain from the best mixed layout down to at most
//!   [`PlanSettings::max_designs`] designs.
//! - [`plan_alternatives`]: the leave-one-out mixed alternatives (best mixed
//!   layout's most-used design removed from the full list, Pareto front
//!   recomputed, DP re-run, up to three times).
//! - [`uniform_layouts`]: every design in every fully filled `a x b x c`
//!   grid of equal cells.
//! - [`refine`] / [`finish_plan`]: continuous optimisation of the cut
//!   positions, then the final ranking ([`merge_and_rank`]).
//! - [`plan_rough`]: all of the above, sequentially.
//! - [`placement`] / [`stone_preform`]: where a planned stone sits in the rough and the piece it
//!   comes from, as the editor's preform (the rigid map between the rough and the design's
//!   frames, and the sawn piece box clipped by the rough's own planes, in model units).
//!
//! The piece table, the alternatives, the single-design pass and the refinement also exist as
//! `_lanes` twins ([`build_piece_table_lanes`], [`plan_alternatives_lanes`],
//! [`uniform_layouts_lanes`], [`finish_plan_lanes`]) that split their work over scoped
//! threads for an application. Each returns bitwise what its serial function returns, for
//! any lane count; the serial function is the twin with one lane, which spawns nothing.
//!
//! # Determinism and portability
//!
//! The standalone block planner ([`plan_rough`]) is single-threaded, with no I/O, no clock, no
//! `HashMap`/`HashSet`, no libm calls beyond IEEE `+ - * /`, `min`/`max`, `round` and
//! `mul_add`: identical input gives bitwise-identical output, and the module builds for
//! `wasm32-unknown-unknown`. The result never depends on the order of the
//! candidate list (it is sorted by `entry_id` first).

pub mod camera_spectral;
pub mod colour_fit;
mod dp;
mod dp_capped;
pub mod fit;
pub mod locate;
mod lp;
mod pareto;
pub mod photometry;
mod piece;
pub mod placement;
pub mod plan;
mod rank;
mod refine;
pub mod shape;
pub mod shaped;
pub mod stone_preform;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_assembly;
#[cfg(test)]
mod tests_brute;
#[cfg(test)]
mod tests_design_cap;
#[cfg(test)]
mod tests_input;
#[cfg(test)]
mod tests_lanes;
#[cfg(test)]
mod tests_lp;
#[cfg(test)]
mod tests_stone_cap;
mod tree;
mod types;
mod uniform;
pub mod zoned_plan;

pub use dp::{plan_alternatives, plan_alternatives_lanes, plan_design_cap, plan_rough_for_order};
pub use fit::*;
pub use pareto::pareto_front;
pub use piece::{Grid, PieceTable, build_piece_table, build_piece_table_lanes, choose_grid};
pub use placement::{
    DesignPlacement, POSE_COUNT, RigidMap, candidate_poses, pose_variant, rough_to_stone_map,
};
pub use plan::*;
pub use rank::{
    best_layout, flatten_groups, merge_and_rank, merge_and_rank_capped, merge_and_rank_within,
    rank_indices, rank_indices_capped, rank_indices_within,
};
pub use refine::{
    final_ranking, final_ranking_capped, final_ranking_within, finish_plan, finish_plan_lanes,
    own_pool, refine, refine_capped,
};
pub use shape::{
    BoxFace, HullError, MAX_HULL_PLANES, MAX_MESH_TRIANGLES, MeshError, RoughBase, RoughCut,
    RoughMeasure, RoughMesh, RoughModel, ShapeError, import_hull, import_mesh,
};
pub use stone_preform::{
    StonePreform, halfspace_to_design_frame, piece_box_halfspaces, stone_preform_planes,
};
pub use types::{
    Axis, BarCut, CandidateDesign, CutOrder, CutPlan, LayoutCaps, LayoutGroup, PlacedStone,
    PlanInputError, PlanProgress, PlanSettings, RoughBlock, RoughLayout, SlabCut,
};
pub use uniform::{uniform_layouts, uniform_layouts_lanes};

/// Layouts in every final ranked list (the block planner's, the shaped planner's and the
/// refinement's top-up); the exact single-stone fit counts in [`plan`] derive from it.
pub const FINAL_TOP: usize = 10;
/// Candidates refined before the final cut (the best 20 of the ranked list).
pub const REFINE_TOP: usize = 20;
/// Designs the shaped uniform pass evaluates.
pub const SHAPED_UNIFORM_DESIGNS: usize = 64;
/// The one per-design-set limit of every ranked list: at most this many layouts may use the
/// same set of designs (counts ignored). A selection that uses a single design therefore
/// shows at most this many layouts.
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
/// cut-order DPs, the design-cap chain, the leave-one-out alternatives, the single-design
/// pass, the refinement of the best 20 and the final ranking. `on_progress` is called
/// throughout with a [`PlanProgress`]; returning `false` cancels.
///
/// The list holds distinct compositions only, and at most three layouts per design set
/// (the `SAME_SET_CAP` of the ranking), each within both caps of the settings, the design cap
/// and the stone cap ([`PlanSettings::max_designs`], [`PlanSettings::max_stones`]): a
/// selection with one design shows at most three, and any selection shows fewer than ten when
/// fewer distinct compositions survive those caps. With a stone cap the searches themselves
/// look for the best layouts of at most that many stones (see the module docs), so the list
/// is the heaviest of that kind, not a larger layout cut down.
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
            on_progress,
        )?);
    }
    let (capped, alternatives) = match rank::best_layout(&mixed) {
        Some(best) => (
            plan_design_cap(
                &grid,
                &clean,
                best,
                settings.max_designs_usize(),
                1,
                on_progress,
            )?,
            plan_alternatives(&grid, &clean, best, on_progress)?,
        ),
        None => (Vec::new(), Vec::new()),
    };
    let mut groups = vec![LayoutGroup {
        pool: front,
        layouts: mixed,
    }];
    groups.extend(capped);
    groups.extend(alternatives);
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform_layouts(rough, settings, &clean, on_progress)?,
    });
    finish_plan(rough, settings, &clean, &groups, on_progress)
}
