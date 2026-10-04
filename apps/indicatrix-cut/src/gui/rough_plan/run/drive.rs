//! One plan, start to finish, on scoped threads.
//!
//! This is the core's sequential `plan()` stage for stage (same stages in the same order,
//! same group order, same ranking); only the independent work inside a stage is spread
//! over lanes and merged in a fixed order. The result is therefore the core's result,
//! bit for bit, for any lane count.

use super::{
    stages::{
        ClipJob, FitJob, build_clipped_table_parallel, fit_single_stones_parallel, refine_parallel,
        run_orders,
    },
    tracker::{Note, Progress},
};
use indicatrix_cut_core::rough_plan::{
    CandidateDesign, CutOrder, FINAL_LAYOUTS, LayoutGroup, PLAIN_SINGLE_FITS, PlanInput, PlanPath,
    PlanProgress, REFINE_TOP, RoughBlock, RoughLayout, SHAPED_SINGLE_FITS, SHAPED_SINGLE_LAYOUTS,
    best_layout, build_piece_table, choose_grid, final_ranking, finish_plan, flatten_groups,
    merge_and_rank, own_pool, pareto_front, plan_alternatives, plan_rough_for_order, rank_indices,
    shaped::{
        ShapedAltParams, ShapedCtx, ShapedGrid, choose_shaped_grid_at, grid_poll_events,
        plan_shaped_alternatives, plan_shaped_for_order, refine_shaped, shaped_uniform_layouts,
    },
    single_fit_layouts, uniform_layouts, uniform_pool,
};

/// DP ticks the plain planner reports per cell of the first cut axis and order.
const PLAIN_TICKS_PER_CELL: usize = 2;

/// DP ticks the shaped planner reports per cell of the first cut axis and order.
const SHAPED_TICKS_PER_CELL: usize = 3;

/// Plans the rough of `input` on up to `lanes` threads. `None` when `progress` cancelled;
/// an empty list when nothing is feasible (or the model is invalid: the caller validates
/// it first to say so).
///
/// `input.designs` must be valid and sorted by entry id, as `candidates_from` yields
/// them.
pub(super) fn drive(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    match input.path() {
        PlanPath::PlainBlock => plan_plain_block(input, lanes, progress),
        PlanPath::Shaped => plan_shaped(input, lanes, progress),
    }
}

/// The DP ticks of all six orders on `cells`.
fn dp_ticks(cells: [usize; 3], ticks_per_cell: usize) -> usize {
    CutOrder::ALL
        .iter()
        .map(|order| ticks_per_cell * cells[order.axes()[0]])
        .sum()
}

/// The `Grid` events of a shaped rough's two piece tables: the size table reports one per
/// plane of the first axis; the clipped table is built one plane of the first axis at a
/// time (see `build_clipped_table_parallel`) and each such build reports one event per
/// `GRID_POLL_PIECES` entries, rounded up.
///
/// The unit is one `PlanProgress::Grid` event, and only the first two tables count: the
/// leave-one-out rounds build their own tables and report their own `Grid` events, which
/// the tracker routes to the alternatives stage once an `Alternatives` event was seen.
/// For the plane `a0` the clipped build covers `(cells[0] - a0)` planes of
/// `range_count(1) * range_count(2)` entries each.
fn shaped_grid_events(grid: &ShapedGrid) -> usize {
    let per_plane = grid.range_count(1) * grid.range_count(2);
    let clipped: usize = (0..grid.cells[0])
        .map(|a0| grid_poll_events((grid.cells[0] - a0) * per_plane))
        .sum();
    grid.cells[0] + clipped
}

/// The plain block: the legacy stages, plus the exact single-stone fits merged into the
/// final list.
fn plan_plain_block(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let settings = input.settings;
    let inset = settings.skin_mm + settings.allowance_mm;
    let (Ok(region), Ok(coarse_region)) = (
        input.model.canonical_usable_halfspaces(inset),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };
    // The front is computed once for the whole run; it is empty exactly when no design is
    // valid.
    let front = pareto_front(input.designs);
    if front.is_empty() {
        return Some(Vec::new());
    }

    let extents = input.model.base.bounding_box_extents();
    let rough = RoughBlock {
        x_mm: extents[0],
        y_mm: extents[1],
        z_mm: extents[2],
    };
    let mut layouts = plan_block_layouts(&rough, input, front, lanes, progress)?;
    let job = FitJob {
        region: &region,
        coarse_region: &coarse_region,
        hulls: input.hulls,
        settings,
        keep: PLAIN_SINGLE_FITS,
        // A plain block is never a mesh rough.
        mesh: None,
    };
    let fits = fit_single_stones_parallel(&job, lanes, progress)?;
    layouts.extend(single_fit_layouts(
        &fits,
        input.hulls,
        PLAIN_SINGLE_FITS,
        rough.volume_mm3(),
    ));
    Some(merge_and_rank(layouts, FINAL_LAYOUTS))
}

/// The core's `plan_rough` over the Pareto `front`, with the six orders on threads.
fn plan_block_layouts(
    rough: &RoughBlock,
    input: &PlanInput<'_>,
    front: Vec<CandidateDesign>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let (settings, designs) = (input.settings, input.designs);
    let mut on_progress = |event: PlanProgress| progress.event(event);
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }
    let grid = choose_grid(rough, settings);
    progress.note(Note::Grid(grid.cells()[0]));
    progress.note(Note::Dp(dp_ticks(grid.cells(), PLAIN_TICKS_PER_CELL)));

    let table = build_piece_table(&grid, &front, &mut on_progress)?;
    let mixed = run_orders(lanes, progress, |order, on| {
        plan_rough_for_order(&grid, &table, &front, order, settings.count, on)
    })?;
    let alternatives = match best_layout(&mixed) {
        // The FULL candidate list, not the front: a design dominated only by a removed
        // one may be needed once that one is gone.
        Some(best) => plan_alternatives(&grid, designs, best, settings.count, &mut on_progress)?,
        None => Vec::new(),
    };

    let mut groups = vec![LayoutGroup {
        pool: front,
        layouts: mixed,
    }];
    groups.extend(alternatives);
    if !on_progress(PlanProgress::Uniform { done: 0, total: 1 }) {
        return None;
    }
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform_layouts(rough, settings, designs, &mut on_progress)?,
    });
    if !on_progress(PlanProgress::Uniform { done: 1, total: 1 }) {
        return None;
    }
    finish_plan(rough, settings, designs, &groups, &mut on_progress)
}

/// A shaped rough: the positional DPs over the clipped table, the leave-one-out
/// alternatives, the exact single-stone fits, the shaped uniform pass, the refinement of
/// the best layouts and the final ranking.
fn plan_shaped(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let mut on_progress = |event: PlanProgress| progress.event(event);
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }
    let designs = input.designs;
    let front = pareto_front(designs);
    if front.is_empty() {
        return Some(Vec::new());
    }
    let settings = input.settings;
    let inset = settings.skin_mm + settings.allowance_mm;
    let (Ok(ctx), Ok(coarse_region)) = (
        ShapedCtx::new(input.model, settings),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };

    let mut groups = shaped_dp_groups(input, &ctx, &front, lanes, progress)?;
    let job = FitJob {
        region: &ctx.usable,
        coarse_region: &coarse_region,
        hulls: input.hulls,
        settings,
        keep: SHAPED_SINGLE_FITS,
        mesh: ctx.fit_mesh(),
    };
    let single_fits = fit_single_stones_parallel(&job, lanes, progress)?;

    let pool = uniform_pool(&front, &single_fits, designs);
    let uniforms = shaped_uniform_layouts(&ctx, &pool, settings, lanes, &mut on_progress)?;
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniforms,
    });

    let (flat, group_of) = flatten_groups(&groups);
    let ranked = rank_indices(&flat, REFINE_TOP);
    let mut refined = refine_parallel(ranked.len(), lanes, progress, |slot| {
        let index = ranked[slot];
        let group = &groups[group_of[index]];
        let pool = if group.pool.is_empty() {
            own_pool(designs, flat[index])
        } else {
            group.pool.clone()
        };
        refine_shaped(&ctx, flat[index], &pool, settings)
    })?;
    refined.extend(single_fit_layouts(
        &single_fits,
        input.hulls,
        SHAPED_SINGLE_LAYOUTS,
        ctx.model_volume,
    ));
    Some(final_ranking(refined, &flat, &ranked))
}

/// The size table, the clipped table, the six DPs and the alternatives of a shaped
/// rough, as refinement groups in the plain planner's order (mixed, alternatives).
fn shaped_dp_groups(
    input: &PlanInput<'_>,
    ctx: &ShapedCtx,
    front: &[CandidateDesign],
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<LayoutGroup>> {
    let settings = input.settings;
    let mut on_progress = |event: PlanProgress| progress.event(event);
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, settings);
    progress.note(Note::Grid(shaped_grid_events(&grid)));
    progress.note(Note::Dp(dp_ticks(grid.cells, SHAPED_TICKS_PER_CELL)));

    let size_table = grid.size_table(settings, front, &mut on_progress)?;
    let clip = ClipJob {
        grid: &grid,
        front,
        non_box: &ctx.non_box,
        size_table: &size_table,
        settings,
        mesh: ctx.mesh.as_deref(),
    };
    let clipped = build_clipped_table_parallel(&clip, lanes, progress)?;
    let mixed = run_orders(lanes, progress, |order, on| {
        plan_shaped_for_order(&grid, &clipped, ctx, front, order, settings, on)
    })?;
    let alternatives = match best_layout(&mixed) {
        Some(best) => {
            let params = ShapedAltParams {
                grid: &grid,
                table: &clipped,
                ctx,
                all_designs: input.designs,
                best,
                settings,
                lanes,
            };
            plan_shaped_alternatives(&params, &mut on_progress)?
        }
        None => Vec::new(),
    };

    let mut groups = vec![LayoutGroup {
        pool: front.to_vec(),
        layouts: mixed,
    }];
    groups.extend(alternatives);
    Some(groups)
}
